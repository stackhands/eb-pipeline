//! `eb-cvmfs` brackets one configured synchronization in a CVMFS transaction.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CVMFS_SERVER: &str = r#"#!/bin/sh
echo "$*" >> "$CALLS"
case "$1" in
  list)
    if [ -n "${STUB_IN_TRANSACTION:-}" ]; then
      echo "$REPOSITORY (stratum0 / local) - in transaction"
    else
      echo "$REPOSITORY (stratum0 / local) - healthy"
    fi
    exit "${STUB_LIST_EXIT:-0}"
    ;;
  transaction) exit "${STUB_TRANSACTION_EXIT:-0}" ;;
  publish) exit "${STUB_PUBLISH_EXIT:-0}" ;;
  abort) exit "${STUB_ABORT_EXIT:-0}" ;;
esac
exit 99
"#;

fn write_exec(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Fixture {
    dir: tempfile::TempDir,
    bin: PathBuf,
    calls: PathBuf,
    sync: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let calls = dir.path().join("calls");
        let sync = dir.path().join("sync");
        std::fs::create_dir(&bin).unwrap();
        write_exec(&bin.join("cvmfs_server"), CVMFS_SERVER);
        write_exec(
            &sync,
            "#!/bin/sh\necho \"sync $*\" >> \"$CALLS\"\nexit \"${SYNC_EXIT:-0}\"\n",
        );
        Self {
            dir,
            bin,
            calls,
            sync,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"));
        command
            .env("PATH", &self.bin)
            .env("CALLS", &self.calls)
            .env("REPOSITORY", "repo.example")
            .args(["repo.example", "--"])
            .arg(&self.sync)
            .arg("--delete");
        command
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(&self.calls).unwrap_or_default()
    }
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn successful_sync_opens_and_publishes_one_transaction() {
    let fixture = Fixture::new();
    let output = fixture.command().output().unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\npublish repo.example\n"
    );
}

#[test]
fn failed_sync_is_aborted_and_its_exit_status_is_returned() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("SYNC_EXIT", "23")
        .env("STUB_ABORT_EXIT", "5")
        .output()
        .unwrap();
    assert_eq!(code(&output), 23);
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\nabort -f repo.example\n"
    );
    assert!(
        stderr(&output).contains("abort failed"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn an_existing_transaction_is_refused_before_sync() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("STUB_IN_TRANSACTION", "1")
        .output()
        .unwrap();
    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("already in a transaction"));
    assert_eq!(fixture.calls(), "list\n");
}

#[test]
fn cvmfs_setup_and_publish_failures_stop_at_the_failing_command() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("STUB_LIST_EXIT", "6")
        .output()
        .unwrap();
    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("cvmfs_server list failed"));
    assert_eq!(fixture.calls(), "list\n");

    std::fs::write(&fixture.calls, "").unwrap();
    let output = fixture
        .command()
        .env("STUB_TRANSACTION_EXIT", "7")
        .output()
        .unwrap();
    assert_eq!(code(&output), 7);
    assert_eq!(fixture.calls(), "list\ntransaction repo.example\n");

    std::fs::write(&fixture.calls, "").unwrap();
    let output = fixture
        .command()
        .env("STUB_PUBLISH_EXIT", "8")
        .output()
        .unwrap();
    assert_eq!(code(&output), 8);
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\npublish repo.example\n"
    );
}

#[test]
fn a_sync_command_that_cannot_start_is_aborted_and_returns_127() {
    let fixture = Fixture::new();
    let missing = fixture.dir.path().join("missing-sync");
    let mut command = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"));
    let output = command
        .env("PATH", &fixture.bin)
        .env("CALLS", &fixture.calls)
        .env("REPOSITORY", "repo.example")
        .args(["repo.example", "--"])
        .arg(&missing)
        .output()
        .unwrap();
    assert_eq!(code(&output), 127);
    assert!(stderr(&output).contains("cannot start sync command"));
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nabort -f repo.example\n"
    );
}

#[test]
fn a_missing_cvmfs_server_is_a_clear_error() {
    let fixture = Fixture::new();
    let empty = fixture.dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let output = fixture.command().env("PATH", empty).output().unwrap();
    assert_eq!(code(&output), 2);
    assert!(
        stderr(&output).contains("cannot start cvmfs_server list"),
        "{}",
        stderr(&output)
    );
    assert!(fixture.calls().is_empty());
}

#[test]
fn dry_run_prints_every_possible_command_without_running_one() {
    let fixture = Fixture::new();
    let output = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .args(["--dry-run", "repo.example", "--"])
        .arg(&fixture.sync)
        .arg("--delete")
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("cvmfs_server list\n"), "{stdout}");
    assert!(
        stdout.contains("cvmfs_server transaction repo.example\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("{} --delete\n", fixture.sync.display())),
        "{stdout}"
    );
    assert!(
        stdout.contains("cvmfs_server publish repo.example\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("cvmfs_server abort -f repo.example\n"),
        "{stdout}"
    );
    assert!(fixture.calls().is_empty());
}

#[test]
fn invalid_arguments_are_usage_errors() {
    for args in [
        vec![],
        vec!["repo.example"],
        vec!["repo.example", "sync"],
        vec!["--unknown", "repo.example", "--", "sync"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(code(&output), 2);
        assert!(output.stdout.is_empty());
    }
}
