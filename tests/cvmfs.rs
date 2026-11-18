//! `eb-cvmfs` brackets one configured synchronization in a CVMFS transaction.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
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
  transaction) [ -n "${STUB_TRANSACTION_DELAY:-}" ] && /bin/sleep "$STUB_TRANSACTION_DELAY"; exit "${STUB_TRANSACTION_EXIT:-0}" ;;
  publish) [ -n "${STUB_PUBLISH_DELAY:-}" ] && /bin/sleep "$STUB_PUBLISH_DELAY"; exit "${STUB_PUBLISH_EXIT:-0}" ;;
  check) exit "${STUB_CHECK_EXIT:-0}" ;;
  abort) [ -n "${STUB_ABORT_DELAY:-}" ] && /bin/sleep "$STUB_ABORT_DELAY"; exit "${STUB_ABORT_EXIT:-0}" ;;
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
    verify: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let calls = dir.path().join("calls");
        let sync = dir.path().join("sync");
        let verify = dir.path().join("verify");
        std::fs::create_dir(&bin).unwrap();
        write_exec(&bin.join("cvmfs_server"), CVMFS_SERVER);
        write_exec(
            &sync,
            "#!/bin/sh\necho \"sync $*\" >> \"$CALLS\"\n[ -n \"${SYNC_DELAY:-}\" ] && /bin/sleep \"$SYNC_DELAY\"\n[ -n \"${REMOVE_SERVER:-}\" ] && /bin/rm -f \"$BIN/cvmfs_server\"\nexit \"${SYNC_EXIT:-0}\"\n",
        );
        write_exec(
            &verify,
            "#!/bin/sh\necho \"verify $*\" >> \"$CALLS\"\necho verify-stdout\necho verify-stderr >&2\nexit \"${VERIFY_EXIT:-0}\"\n",
        );
        Self {
            dir,
            bin,
            calls,
            sync,
            verify,
        }
    }

    fn command(&self) -> Command {
        self.command_with_options(None, None, None, &[])
    }

    fn command_with_metrics(&self, metrics: Option<&Path>) -> Command {
        self.command_with_options(None, metrics, None, &[])
    }

    fn command_with_verify(&self, verify: Option<&str>) -> Command {
        self.command_with_options(None, None, verify, &[])
    }

    fn command_with(&self, manifest: Option<&Path>, metrics: Option<&Path>) -> Command {
        self.command_with_options(manifest, metrics, None, &[])
    }

    fn command_with_options(
        &self,
        manifest: Option<&Path>,
        metrics: Option<&Path>,
        verify: Option<&str>,
        extra: &[&str],
    ) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"));
        command
            .env("PATH", &self.bin)
            .env("BIN", &self.bin)
            .env("CALLS", &self.calls)
            .env("REPOSITORY", "repo.example");
        if let Some(manifest) = manifest {
            command.arg("--archive-manifest").arg(manifest);
        }
        if let Some(metrics) = metrics {
            command.arg("--metrics-file").arg(metrics);
        }
        command.args(extra);
        if let Some(verify) = verify {
            command.args(["--verify-cmd", verify]);
        }
        command
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

fn sample(text: &str, family: &str) -> Option<f64> {
    text.lines()
        .find(|line| line.starts_with(&format!("{family}{{")))
        .and_then(|line| line.split_whitespace().last())
        .and_then(|value| value.parse().ok())
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
fn successful_publish_writes_final_metrics_atomically_with_the_existing_mode() {
    let fixture = Fixture::new();
    let metrics = fixture.dir.path().join("cvmfs.prom");
    std::fs::write(&metrics, "stale\n").unwrap();
    std::fs::set_permissions(&metrics, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut command = fixture.command_with_metrics(Some(&metrics));
    command
        .env("STUB_TRANSACTION_DELAY", "0.1")
        .env("STUB_PUBLISH_DELAY", "0.1");
    // SAFETY: `umask` is async-signal-safe and the closure allocates nothing.
    unsafe {
        command.pre_exec(|| {
            libc::umask(0o077);
            Ok(())
        });
    }
    let output = command.output().unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));

    let text = std::fs::read_to_string(&metrics).unwrap();
    assert_eq!(sample(&text, "eb_cvmfs_last_result"), Some(1.0));
    assert_eq!(sample(&text, "eb_cvmfs_exit_code"), Some(0.0));
    assert_eq!(sample(&text, "eb_cvmfs_aborts"), Some(0.0));
    assert!(
        sample(&text, "eb_cvmfs_last_publish_timestamp_seconds").unwrap() > 0.0,
        "{text}"
    );
    assert!(
        sample(&text, "eb_cvmfs_last_transaction_duration_seconds").unwrap() >= 0.2,
        "{text}"
    );
    assert_eq!(
        std::fs::metadata(&metrics).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert!(!std::fs::read_dir(fixture.dir.path())
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
}

#[test]
fn failed_sync_preserves_the_last_publish_and_records_its_completed_abort() {
    let fixture = Fixture::new();
    let metrics = fixture.dir.path().join("cvmfs.prom");
    let output = fixture
        .command_with_metrics(Some(&metrics))
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let first = std::fs::read_to_string(&metrics).unwrap();
    let published = sample(&first, "eb_cvmfs_last_publish_timestamp_seconds").unwrap();

    let output = fixture
        .command_with_metrics(Some(&metrics))
        .env("SYNC_EXIT", "23")
        .env("STUB_ABORT_DELAY", "0.1")
        .output()
        .unwrap();
    assert_eq!(code(&output), 23, "{}", stderr(&output));
    let text = std::fs::read_to_string(&metrics).unwrap();
    assert_eq!(sample(&text, "eb_cvmfs_last_result"), Some(0.0));
    assert_eq!(sample(&text, "eb_cvmfs_exit_code"), Some(23.0));
    assert_eq!(sample(&text, "eb_cvmfs_aborts"), Some(1.0));
    assert_eq!(
        sample(&text, "eb_cvmfs_last_publish_timestamp_seconds"),
        Some(published)
    );
    assert!(
        sample(&text, "eb_cvmfs_last_transaction_duration_seconds").unwrap() >= 0.1,
        "{text}"
    );
}

#[test]
fn an_abort_that_cannot_start_is_not_counted_or_called_complete() {
    let fixture = Fixture::new();
    let metrics = fixture.dir.path().join("cvmfs.prom");
    let output = fixture
        .command_with_metrics(Some(&metrics))
        .env("SYNC_EXIT", "23")
        .env("REMOVE_SERVER", "1")
        .output()
        .unwrap();
    assert_eq!(code(&output), 23, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("cannot start cvmfs_server abort"),
        "{}",
        stderr(&output)
    );
    let text = std::fs::read_to_string(metrics).unwrap();
    assert_eq!(sample(&text, "eb_cvmfs_aborts"), Some(0.0));
    assert_eq!(
        sample(&text, "eb_cvmfs_last_transaction_duration_seconds"),
        None
    );
}

#[test]
fn a_metrics_write_failure_does_not_mask_the_sync_failure() {
    let fixture = Fixture::new();
    let metrics = fixture.dir.path().join("cvmfs.prom");
    std::fs::create_dir(&metrics).unwrap();
    let output = fixture
        .command_with_metrics(Some(&metrics))
        .env("SYNC_EXIT", "23")
        .output()
        .unwrap();
    assert_eq!(code(&output), 23, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("cannot write metrics"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\nabort -f repo.example\n"
    );
}

#[test]
fn a_failed_publish_has_duration_but_no_success_timestamp() {
    let fixture = Fixture::new();
    let metrics = fixture.dir.path().join("cvmfs.prom");
    let output = fixture
        .command_with_metrics(Some(&metrics))
        .env("STUB_PUBLISH_EXIT", "8")
        .output()
        .unwrap();
    assert_eq!(code(&output), 8, "{}", stderr(&output));
    let text = std::fs::read_to_string(metrics).unwrap();
    assert_eq!(sample(&text, "eb_cvmfs_last_result"), Some(0.0));
    assert_eq!(sample(&text, "eb_cvmfs_exit_code"), Some(8.0));
    assert_eq!(
        sample(&text, "eb_cvmfs_last_publish_timestamp_seconds"),
        None
    );
    assert!(
        sample(&text, "eb_cvmfs_last_transaction_duration_seconds").is_some(),
        "{text}"
    );
}

#[test]
fn a_shared_metrics_file_serializes_whole_invocations() {
    let fixture = Fixture::new();
    let metrics = fixture.dir.path().join("cvmfs.prom");
    let mut first = fixture.command_with_metrics(Some(&metrics));
    first.env("SYNC_DELAY", "0.2");
    let first = first.spawn().unwrap();
    for _ in 0..100 {
        if fixture.calls().contains("sync --delete") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(fixture.calls().contains("sync --delete"));

    let second = fixture
        .command_with_metrics(Some(&metrics))
        .spawn()
        .unwrap();
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();
    assert_eq!(code(&first), 0, "{}", stderr(&first));
    assert_eq!(code(&second), 0, "{}", stderr(&second));
    assert_eq!(
        fixture.calls(),
        "\
list
transaction repo.example
sync --delete
publish repo.example
list
transaction repo.example
sync --delete
publish repo.example
"
    );
}

#[test]
fn successful_verification_runs_between_sync_and_publish_without_a_shell() {
    let fixture = Fixture::new();
    let verify = format!("{} --cache 'two words'", fixture.verify.display());
    let output = fixture.command_with_verify(Some(&verify)).output().unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\nverify --cache two words\npublish repo.example\n"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "verify-stdout\n");
    assert_eq!(stderr(&output), "verify-stderr\n");
}

#[test]
fn failed_verification_is_aborted_and_preserves_its_output_and_status() {
    let fixture = Fixture::new();
    let verify = fixture.verify.display().to_string();
    let output = fixture
        .command_with_verify(Some(&verify))
        .env("VERIFY_EXIT", "29")
        .output()
        .unwrap();
    assert_eq!(code(&output), 29);
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\nverify \nabort -f repo.example\n"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "verify-stdout\n");
    assert_eq!(stderr(&output), "verify-stderr\n");
}

#[test]
fn verification_uses_a_shell_when_the_command_contains_shell_syntax() {
    let fixture = Fixture::new();
    let verify = "printf 'verify shell\\n' >> \"$CALLS\" && true";
    let path = std::env::var("PATH").unwrap();
    let output = fixture
        .command_with_verify(Some(verify))
        .env("PATH", format!("{}:{path}", fixture.bin.display()))
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\nverify shell\npublish repo.example\n"
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
fn failed_sync_does_not_run_verification() {
    let fixture = Fixture::new();
    let verify = fixture.verify.display().to_string();
    let output = fixture
        .command_with_verify(Some(&verify))
        .env("SYNC_EXIT", "23")
        .output()
        .unwrap();
    assert_eq!(code(&output), 23);
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\nabort -f repo.example\n"
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
fn a_verification_command_that_cannot_start_is_aborted_and_returns_127() {
    let fixture = Fixture::new();
    let missing = fixture.dir.path().join("missing-verify");
    let output = fixture
        .command_with_verify(Some(&missing.display().to_string()))
        .output()
        .unwrap();
    assert_eq!(code(&output), 127);
    assert!(stderr(&output).contains("cannot start verification command"));
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\nabort -f repo.example\n"
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
    let sync = format!("{} --delete", fixture.sync.display());
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        [
            "cvmfs_server list",
            "cvmfs_server transaction repo.example",
            &sync,
            "cvmfs_server publish repo.example",
            "# on sync failure: cvmfs_server abort -f repo.example",
        ]
    );
    assert!(fixture.calls().is_empty());
}

#[test]
fn dry_run_prints_verification_between_sync_and_publish() {
    let fixture = Fixture::new();
    let verify = format!("{} --cache 'two words'", fixture.verify.display());
    let output = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .args(["--dry-run", "--verify-cmd", &verify, "repo.example", "--"])
        .arg(&fixture.sync)
        .arg("--delete")
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let sync = format!("{} --delete", fixture.sync.display());
    let verifier = format!("{} --cache 'two words'", fixture.verify.display());
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        [
            "cvmfs_server list",
            "cvmfs_server transaction repo.example",
            &sync,
            &verifier,
            "cvmfs_server publish repo.example",
            "# on sync or verification failure: cvmfs_server abort -f repo.example",
        ]
    );
    assert!(fixture.calls().is_empty());
}

fn historical_dry_run(sync: &str) -> Vec<u8> {
    format!(
        "cvmfs_server list\n\
         cvmfs_server transaction repo.example\n\
         {sync} --delete\n\
         cvmfs_server publish repo.example\n\
         # on sync failure: cvmfs_server abort -f repo.example\n"
    )
    .into_bytes()
}

const HISTORICAL_CALLS: &[u8] =
    b"list\ntransaction repo.example\nsync --delete\npublish repo.example\n";

#[test]
fn default_outputs_match_the_bytes_from_before_archive_manifests() {
    let sync = "/opt/site/bin/sync-module-cache";
    let empty = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .env("PATH", empty.path())
        .args(["--dry-run", "repo.example", "--", sync, "--delete"])
        .output()
        .unwrap();
    assert_eq!(code(&output), 0);
    assert_eq!(output.stdout, historical_dry_run(sync));
    assert_eq!(output.stderr, b"");

    let fixture = Fixture::new();
    let output = fixture.command().output().unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"");
    assert_eq!(fixture.calls().as_bytes(), HISTORICAL_CALLS);
    let names: Vec<String> = std::fs::read_dir(fixture.dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names
            .iter()
            .all(|name| !name.ends_with(".prom") && !name.ends_with(".lock")),
        "omitting both options writes no metrics file: {names:?}"
    );

    let fixture = Fixture::new();
    let metrics = fixture.dir.path().join("cvmfs.prom");
    let output = fixture
        .command_with_metrics(Some(&metrics))
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"");
    assert_eq!(fixture.calls().as_bytes(), HISTORICAL_CALLS);
    let text = std::fs::read_to_string(&metrics).unwrap();
    assert_eq!(sample(&text, "eb_cvmfs_last_result"), Some(1.0));
    assert_eq!(sample(&text, "eb_cvmfs_exit_code"), Some(0.0));
    assert_eq!(sample(&text, "eb_cvmfs_aborts"), Some(0.0));
    assert!(
        sample(&text, "eb_cvmfs_last_publish_timestamp_seconds").unwrap() > 0.0,
        "{text}"
    );
    assert!(sample(&text, "eb_cvmfs_last_transaction_duration_seconds").is_some());
    assert!(fixture.dir.path().join(".cvmfs.prom.lock").is_file());
}

#[test]
fn archive_manifest_after_the_separator_stays_a_sync_argument() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .arg("--archive-manifest")
        .arg("ingest.json")
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete --archive-manifest ingest.json\npublish repo.example\n"
    );
    assert!(!stderr(&output).contains("archive manifest"));
}

#[test]
fn a_refused_archive_does_not_open_a_transaction_or_change_dry_run_bytes() {
    let fixture = Fixture::new();
    let tree = fixture.dir.path().join("tree");
    std::fs::create_dir_all(tree.join("pkg")).unwrap();
    std::fs::write(tree.join("pkg/file"), b"hi\n").unwrap();
    std::fs::write(fixture.dir.path().join("list"), "pkg/file\n").unwrap();
    let flat = fixture.dir.path().join("ingest");
    std::fs::create_dir(&flat).unwrap();
    let archive = flat.join("cache.tar");
    let status = Command::new("tar")
        .args([
            "--format=ustar",
            "-cf",
            archive.to_str().unwrap(),
            "-C",
            tree.to_str().unwrap(),
            "--no-recursion",
            "--files-from",
            fixture.dir.path().join("list").to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let manifest = manifest_for(&fixture, "cache.tar", "i32");

    let output = fixture
        .command_with(Some(&manifest), None)
        .output()
        .unwrap();
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).contains("missing explicit directory `pkg`"));
    assert!(fixture.calls().is_empty());

    let preview = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .env("PATH", &fixture.bin)
        .args(["--dry-run", "--archive-manifest"])
        .arg(&manifest)
        .args([
            "repo.example",
            "--",
            "/opt/site/bin/sync-module-cache",
            "--delete",
        ])
        .output()
        .unwrap();
    assert_eq!(code(&preview), 2);
    assert!(preview.stdout.is_empty(), "{}", stderr(&preview));
    assert!(fixture.calls().is_empty());
}

#[test]
fn an_accepted_archive_keeps_the_historical_command_bytes() {
    let fixture = Fixture::new();
    let tree = fixture.dir.path().join("tree");
    std::fs::create_dir_all(tree.join("pkg")).unwrap();
    std::fs::write(tree.join("pkg/file"), b"hi\n").unwrap();
    let ingest = fixture.dir.path().join("ingest");
    std::fs::create_dir(&ingest).unwrap();
    let archive = ingest.join("cache.tar");
    let status = Command::new("tar")
        .args(["--format=ustar", "-cf"])
        .arg(&archive)
        .arg("-C")
        .arg(&tree)
        .arg("pkg")
        .status()
        .unwrap();
    assert!(status.success());
    let manifest = manifest_for(&fixture, "cache.tar", "i32");

    let output = fixture
        .command_with(Some(&manifest), None)
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"");
    assert_eq!(fixture.calls().as_bytes(), HISTORICAL_CALLS);

    let sync = "/opt/site/bin/sync-module-cache";
    let preview = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .env("PATH", &fixture.bin)
        .args(["--dry-run", "--archive-manifest"])
        .arg(&manifest)
        .args(["repo.example", "--", sync, "--delete"])
        .output()
        .unwrap();
    assert_eq!(code(&preview), 0, "{}", stderr(&preview));
    assert_eq!(preview.stdout, historical_dry_run(sync));
    assert_eq!(preview.stderr, b"");
    assert!(fixture.calls().as_bytes() == HISTORICAL_CALLS);
}

#[test]
fn an_accepted_archive_composes_with_verification_tags_and_check() {
    let fixture = Fixture::new();
    let tree = fixture.dir.path().join("tree");
    std::fs::create_dir_all(tree.join("pkg")).unwrap();
    std::fs::write(tree.join("pkg/file"), b"hi\n").unwrap();
    let ingest = fixture.dir.path().join("ingest");
    std::fs::create_dir(&ingest).unwrap();
    let archive = ingest.join("cache.tar");
    let status = Command::new("tar")
        .args(["--format=ustar", "-cf"])
        .arg(&archive)
        .arg("-C")
        .arg(&tree)
        .arg("pkg")
        .status()
        .unwrap();
    assert!(status.success());
    let manifest = manifest_for(&fixture, "cache.tar", "i32");
    let verify = fixture.verify.display().to_string();

    let output = fixture
        .command_with_options(
            Some(&manifest),
            None,
            Some(&verify),
            &[
                "--tag",
                "snapshot",
                "--tag-description",
                "checked archive",
                "--check",
            ],
        )
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\nverify \npublish -a snapshot -m checked archive repo.example\ncheck repo.example\n"
    );
}

fn manifest_for(fixture: &Fixture, name: &str, id_type: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(fixture.dir.path().join("ingest").join(name)).unwrap();
    let digest = Sha256::digest(bytes);
    let mut sha = String::with_capacity(64);
    for byte in digest {
        sha.push_str(&format!("{byte:02x}"));
    }
    let path = fixture.dir.path().join("manifest.json");
    std::fs::write(
        &path,
        format!(
            "{{\n  \"directory\": \"ingest\",\n  \"name\": \"{name}\",\n  \"sha256\": \"{sha}\",\n  \"uid_type\": \"{id_type}\",\n  \"gid_type\": \"{id_type}\"\n}}\n"
        ),
    )
    .unwrap();
    path
}

#[test]
fn dry_run_updates_only_invocation_metrics() {
    let fixture = Fixture::new();
    let metrics = fixture.dir.path().join("cvmfs.prom");
    let output = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .args(["--dry-run", "--metrics-file"])
        .arg(&metrics)
        .args(["repo.example", "--"])
        .arg(&fixture.sync)
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = std::fs::read_to_string(metrics).unwrap();
    assert_eq!(sample(&text, "eb_cvmfs_last_result"), Some(1.0));
    assert_eq!(sample(&text, "eb_cvmfs_exit_code"), Some(0.0));
    assert_eq!(sample(&text, "eb_cvmfs_aborts"), Some(0.0));
    assert_eq!(
        sample(&text, "eb_cvmfs_last_publish_timestamp_seconds"),
        None
    );
    assert_eq!(
        sample(&text, "eb_cvmfs_last_transaction_duration_seconds"),
        None
    );
    assert!(fixture.calls().is_empty());
}

#[test]
fn publish_passes_named_tag_and_description_to_cvmfs_server() {
    let fixture = Fixture::new();
    let output = fixture
        .command_with_options(
            None,
            None,
            None,
            &[
                "--tag",
                "release-2026-03",
                "--tag-description",
                "module cache refresh",
            ],
        )
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\npublish -a release-2026-03 -m module cache refresh repo.example\n"
    );
}

#[test]
fn publish_check_runs_after_a_successful_publish() {
    let fixture = Fixture::new();
    let output = fixture
        .command_with_options(None, None, None, &["--check"])
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\npublish repo.example\ncheck repo.example\n"
    );
}

#[test]
fn a_failed_publish_does_not_run_check() {
    let fixture = Fixture::new();
    let output = fixture
        .command_with_options(None, None, None, &["--check"])
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
fn check_failure_returns_its_status_after_publish() {
    let fixture = Fixture::new();
    let output = fixture
        .command_with_options(None, None, None, &["--check"])
        .env("STUB_CHECK_EXIT", "11")
        .output()
        .unwrap();
    assert_eq!(code(&output), 11);
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\npublish repo.example\ncheck repo.example\n"
    );
}

#[test]
fn tag_options_appear_in_dry_run_publish_and_check_order() {
    let fixture = Fixture::new();
    let output = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .args([
            "--dry-run",
            "--tag",
            "snapshot",
            "--tag-description",
            "two words",
            "--check",
            "repo.example",
            "--",
        ])
        .arg(&fixture.sync)
        .arg("--delete")
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let sync = format!("{} --delete", fixture.sync.display());
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        [
            "cvmfs_server list",
            "cvmfs_server transaction repo.example",
            &sync,
            "cvmfs_server publish -a snapshot -m 'two words' repo.example",
            "cvmfs_server check repo.example",
            "# on sync failure: cvmfs_server abort -f repo.example",
        ]
    );
    assert!(fixture.calls().is_empty());
}

#[test]
fn invalid_arguments_are_usage_errors() {
    for args in [
        vec![],
        vec!["repo.example"],
        vec!["repo.example", "sync"],
        vec!["--metrics-file"],
        vec!["--unknown", "repo.example", "--", "sync"],
        vec!["--verify-cmd"],
        vec!["--verify-cmd", "", "repo.example", "--", "sync"],
        vec![
            "--verify-cmd",
            "verify",
            "--verify-cmd",
            "again",
            "repo.example",
            "--",
            "sync",
        ],
        vec![
            "--verify-cmd",
            "unterminated '",
            "repo.example",
            "--",
            "sync",
        ],
        vec!["--tag", "repo.example", "--", "sync"],
        vec!["--tag", "", "repo.example", "--", "sync"],
        vec!["--tag", "a", "--tag", "b", "repo.example", "--", "sync"],
        vec!["--tag-description", "note", "repo.example", "--", "sync"],
        vec![
            "--tag-description",
            "",
            "--tag",
            "a",
            "repo.example",
            "--",
            "sync",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(code(&output), 2);
        assert!(output.stdout.is_empty());
    }
}
