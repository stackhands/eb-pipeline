//! `eb-cvmfs` brackets one configured synchronization in a CVMFS transaction.

#[path = "common/exec_stub.rs"]
mod exec_stub;

use exec_stub::CommandSyncExt;
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
  publish)
    if [ -n "${STUB_PUBLISH_GROUP:-}" ]; then
      set -- $(/bin/cat /proc/$$/stat)
      echo "$$ ${5}" > "$STUB_PUBLISH_GROUP"
    fi
    if [ -n "${STUB_PUBLISH_CHILD_IDENTITY:-}" ]; then
      trap '' TERM
      /bin/sh -c 'trap "" TERM; set -- $(/bin/cat /proc/$$/stat); echo "$$ ${22}" > "$STUB_PUBLISH_CHILD_IDENTITY"; exec /bin/sleep 30' &
      wait
    fi
    [ -n "${STUB_PUBLISH_DELAY:-}" ] && /bin/sleep "$STUB_PUBLISH_DELAY"
    exit "${STUB_PUBLISH_EXIT:-0}"
    ;;
  check)
    if [ -n "${STUB_CHECK_GROUP:-}" ]; then
      set -- $(/bin/cat /proc/$$/stat)
      echo "$$ ${5}" > "$STUB_CHECK_GROUP"
    fi
    if [ -n "${STUB_CHECK_CHILD_IDENTITY:-}" ]; then
      trap '' TERM
      /bin/sh -c 'trap "" TERM; set -- $(/bin/cat /proc/$$/stat); echo "$$ ${22}" > "$STUB_CHECK_CHILD_IDENTITY"; exec /bin/sleep 30' &
      wait
    fi
    [ -n "${STUB_CHECK_DELAY:-}" ] && /bin/sleep "$STUB_CHECK_DELAY"
    exit "${STUB_CHECK_EXIT:-0}"
    ;;
  abort) [ -n "${STUB_ABORT_DELAY:-}" ] && /bin/sleep "$STUB_ABORT_DELAY"; exit "${STUB_ABORT_EXIT:-0}" ;;
esac
exit 99
"#;

fn write_exec(path: &Path, text: &str) {
    exec_stub::write_exec(path, text);
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
            "#!/bin/sh\necho \"verify $*\" >> \"$CALLS\"\necho verify-stdout\necho verify-stderr >&2\n[ -n \"${VERIFY_DELAY:-}\" ] && /bin/sleep \"$VERIFY_DELAY\"\nexit \"${VERIFY_EXIT:-0}\"\n",
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
    let output = fixture.command().sync_output();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\npublish repo.example\n"
    );
}

#[test]
fn sync_without_a_timeout_stays_in_the_callers_process_group() {
    let fixture = Fixture::new();
    let group = fixture.dir.path().join("group");
    write_exec(
        &fixture.sync,
        "#!/bin/sh\n\
         echo \"sync $*\" >> \"$CALLS\"\n\
         set -- $(/bin/cat /proc/$$/stat)\n\
         echo \"$$ ${5}\" > \"$GROUP\"\n",
    );
    let output = fixture.command().env("GROUP", &group).sync_output();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let identity = std::fs::read_to_string(group).unwrap();
    let (pid, process_group) = identity.trim().split_once(' ').unwrap();
    assert_ne!(pid, process_group);
    assert_eq!(process_group, current_process_group());
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"");
}

#[test]
fn publish_without_a_timeout_stays_in_the_callers_process_group() {
    let fixture = Fixture::new();
    let group = fixture.dir.path().join("publish-group");
    let output = fixture
        .command()
        .env("STUB_PUBLISH_GROUP", &group)
        .sync_output();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let identity = std::fs::read_to_string(group).unwrap();
    let (pid, process_group) = identity.trim().split_once(' ').unwrap();
    assert_ne!(pid, process_group);
    assert_eq!(process_group, current_process_group());
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"");
    assert_eq!(fixture.calls().as_bytes(), HISTORICAL_CALLS);
}

#[test]
fn check_without_a_timeout_stays_in_the_callers_process_group() {
    let fixture = Fixture::new();
    let group = fixture.dir.path().join("check-group");
    let output = fixture
        .command_with_options(None, None, None, &["--check"])
        .env("STUB_CHECK_GROUP", &group)
        .sync_output();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let identity = std::fs::read_to_string(group).unwrap();
    let (pid, process_group) = identity.trim().split_once(' ').unwrap();
    assert_ne!(pid, process_group);
    assert_eq!(process_group, current_process_group());
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"");
    assert_eq!(
        fixture.calls().as_bytes(),
        b"list\ntransaction repo.example\nsync --delete\npublish repo.example\ncheck repo.example\n"
    );
}

fn current_process_group() -> String {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let (_, fields) = stat.rsplit_once(") ").unwrap();
    fields.split_whitespace().nth(2).unwrap().to_string()
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
    let output = command.sync_output();
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
    let output = fixture.command_with_metrics(Some(&metrics)).sync_output();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let first = std::fs::read_to_string(&metrics).unwrap();
    let published = sample(&first, "eb_cvmfs_last_publish_timestamp_seconds").unwrap();

    let output = fixture
        .command_with_metrics(Some(&metrics))
        .env("SYNC_EXIT", "23")
        .env("STUB_ABORT_DELAY", "0.1")
        .sync_output();
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
fn signaled_sync_preserves_128_plus_signal_exit_code() {
    let fixture = Fixture::new();
    let metrics = fixture.dir.path().join("cvmfs.prom");
    write_exec(
        &fixture.sync,
        "#!/bin/sh\necho \"sync $*\" >> \"$CALLS\"\nkill -TERM \"$$\"\n",
    );

    let output = fixture.command_with_metrics(Some(&metrics)).sync_output();

    assert_eq!(code(&output), 143, "{}", stderr(&output));
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\nabort -f repo.example\n"
    );
    let text = std::fs::read_to_string(metrics).unwrap();
    assert_eq!(sample(&text, "eb_cvmfs_exit_code"), Some(143.0));
}

#[test]
fn an_abort_that_cannot_start_is_not_counted_or_called_complete() {
    let fixture = Fixture::new();
    let metrics = fixture.dir.path().join("cvmfs.prom");
    let output = fixture
        .command_with_metrics(Some(&metrics))
        .env("SYNC_EXIT", "23")
        .env("REMOVE_SERVER", "1")
        .sync_output();
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
        .sync_output();
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
        .sync_output();
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
    let ready = fixture.dir.path().join("sync.ready");
    let release = fixture.dir.path().join("sync.release");
    write_exec(
        &fixture.sync,
        "#!/bin/sh\n\
         echo \"sync $*\" >> \"$CALLS\"\n\
         if [ -n \"${SYNC_READY:-}\" ]; then\n\
           : > \"$SYNC_READY\"\n\
           while [ ! -e \"$SYNC_RELEASE\" ]; do /bin/sleep 0.01; done\n\
         fi\n",
    );
    let mut first = fixture.command_with_metrics(Some(&metrics));
    first
        .env("SYNC_READY", &ready)
        .env("SYNC_RELEASE", &release);
    let first = first.sync_spawn();
    let became_ready = wait_for_path(&ready, std::time::Duration::from_secs(10));
    if !became_ready {
        std::fs::write(&release, b"go\n").unwrap();
    }
    assert!(
        became_ready,
        "first synchronization did not become ready; calls:\n{}",
        fixture.calls()
    );
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\n"
    );

    let mut second = fixture.command_with_metrics(Some(&metrics)).sync_spawn();
    std::thread::sleep(std::time::Duration::from_millis(50));
    let second_status = second.try_wait();
    let calls_while_locked = fixture.calls();
    std::fs::write(release, b"go\n").unwrap();
    assert!(
        second_status.unwrap().is_none(),
        "second invocation finished while the first held the metrics lock"
    );
    assert_eq!(
        calls_while_locked,
        "list\ntransaction repo.example\nsync --delete\n"
    );
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

fn wait_for_path(path: &Path, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while !path.exists() {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    true
}

#[test]
fn successful_verification_runs_between_sync_and_publish_without_a_shell() {
    let fixture = Fixture::new();
    let verify = format!("{} --cache 'two words'", fixture.verify.display());
    let output = fixture.command_with_verify(Some(&verify)).sync_output();
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
        .sync_output();
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
        .sync_output();
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
        .sync_output();
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
        .sync_output();
    assert_eq!(code(&output), 23);
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\nabort -f repo.example\n"
    );
}

#[test]
fn a_sync_timeout_kills_its_process_group_aborts_once_and_records_the_phase() {
    let fixture = Fixture::new();
    let child_identity = fixture.dir.path().join("child.identity");
    write_exec(
        &fixture.sync,
        "#!/bin/sh\n\
         echo \"sync $*\" >> \"$CALLS\"\n\
         trap '' TERM\n\
         /bin/sh -c 'trap \"\" TERM; set -- $(/bin/cat /proc/$$/stat); echo \"$$ ${22}\" > \"$CHILD_IDENTITY\"; exec /bin/sleep 30' &\n\
         wait\n",
    );
    let metrics = fixture.dir.path().join("cvmfs.prom");
    let output = fixture
        .command_with_options(None, Some(&metrics), None, &["--sync-timeout", "1"])
        .env("CHILD_IDENTITY", &child_identity)
        .sync_output();
    assert_eq!(code(&output), 124, "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    assert!(
        stderr(&output).contains(
            "sync command timed out after 1 second; sent TERM, waited 1 second, then sent KILL"
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\nabort -f repo.example\n"
    );
    let identity = std::fs::read_to_string(child_identity).unwrap();
    let (pid, start_time) = identity.trim().split_once(' ').unwrap();
    let pid: i32 = pid.parse().unwrap();
    let start_time: u64 = start_time.parse().unwrap();
    assert!(
        !same_live_process(pid, start_time),
        "sleep descendant {pid} survived timeout"
    );

    let text = std::fs::read_to_string(metrics).unwrap();
    assert_eq!(sample(&text, "eb_cvmfs_last_result"), Some(0.0));
    assert_eq!(sample(&text, "eb_cvmfs_exit_code"), Some(124.0));
    assert_eq!(sample(&text, "eb_cvmfs_aborts"), Some(1.0));
    assert!(
        text.contains("eb_cvmfs_timeout{repository=\"repo.example\",phase=\"sync\"} 1"),
        "{text}"
    );
}

fn same_live_process(pid: i32, start_time: u64) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some((_, fields)) = stat.rsplit_once(") ") else {
        return false;
    };
    let fields: Vec<_> = fields.split_whitespace().collect();
    fields.first() != Some(&"Z")
        && fields
            .get(19)
            .and_then(|field| field.parse::<u64>().ok())
            .is_some_and(|current| current == start_time)
}

#[test]
fn a_verification_timeout_aborts_without_retrying_or_publishing() {
    let fixture = Fixture::new();
    let verify = fixture.verify.display().to_string();
    let metrics = fixture.dir.path().join("cvmfs.prom");
    let output = fixture
        .command_with_options(
            None,
            Some(&metrics),
            Some(&verify),
            &["--verify-timeout", "1"],
        )
        .env("VERIFY_DELAY", "30")
        .sync_output();
    assert_eq!(code(&output), 124);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "verify-stdout\n");
    assert!(
        stderr(&output).contains("verification command timed out after 1 second"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\nverify \nabort -f repo.example\n"
    );
    let text = std::fs::read_to_string(metrics).unwrap();
    assert_eq!(sample(&text, "eb_cvmfs_exit_code"), Some(124.0));
    assert!(
        text.contains("eb_cvmfs_timeout{repository=\"repo.example\",phase=\"verify\"} 1"),
        "{text}"
    );
}

#[test]
fn a_publish_timeout_kills_its_process_group_aborts_once_and_skips_check() {
    let fixture = Fixture::new();
    let child_identity = fixture.dir.path().join("publish-child.identity");
    let metrics = fixture.dir.path().join("cvmfs.prom");
    let output = fixture
        .command_with_options(
            None,
            Some(&metrics),
            None,
            &["--publish-timeout", "1", "--check"],
        )
        .env("STUB_PUBLISH_CHILD_IDENTITY", &child_identity)
        .sync_output();
    assert_eq!(code(&output), 124, "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    assert!(
        stderr(&output).contains(
            "publish command timed out after 1 second; sent TERM, waited 1 second, then sent KILL"
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\npublish repo.example\nabort -f repo.example\n"
    );
    let identity = std::fs::read_to_string(child_identity).unwrap();
    let (pid, start_time) = identity.trim().split_once(' ').unwrap();
    let pid: i32 = pid.parse().unwrap();
    let start_time: u64 = start_time.parse().unwrap();
    assert!(
        !same_live_process(pid, start_time),
        "publish descendant {pid} survived timeout"
    );

    let text = std::fs::read_to_string(metrics).unwrap();
    assert_eq!(sample(&text, "eb_cvmfs_last_result"), Some(0.0));
    assert_eq!(sample(&text, "eb_cvmfs_exit_code"), Some(124.0));
    assert_eq!(sample(&text, "eb_cvmfs_aborts"), Some(1.0));
    assert!(
        sample(&text, "eb_cvmfs_last_publish_timestamp_seconds").is_none(),
        "{text}"
    );
    assert!(
        text.contains("eb_cvmfs_timeout{repository=\"repo.example\",phase=\"publish\"} 1"),
        "{text}"
    );
}

#[test]
fn a_check_timeout_kills_its_process_group_without_aborting_the_publish() {
    let fixture = Fixture::new();
    let child_identity = fixture.dir.path().join("check-child.identity");
    let metrics = fixture.dir.path().join("cvmfs.prom");
    let output = fixture
        .command_with_options(
            None,
            Some(&metrics),
            None,
            &["--check", "--check-timeout", "1"],
        )
        .env("STUB_CHECK_CHILD_IDENTITY", &child_identity)
        .sync_output();
    assert_eq!(code(&output), 124, "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    assert!(
        stderr(&output).contains(
            "check command timed out after 1 second; sent TERM, waited 1 second, then sent KILL"
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        fixture.calls(),
        "list\ntransaction repo.example\nsync --delete\npublish repo.example\ncheck repo.example\n"
    );
    let identity = std::fs::read_to_string(child_identity).unwrap();
    let (pid, start_time) = identity.trim().split_once(' ').unwrap();
    let pid: i32 = pid.parse().unwrap();
    let start_time: u64 = start_time.parse().unwrap();
    assert!(
        !same_live_process(pid, start_time),
        "check descendant {pid} survived timeout"
    );

    let text = std::fs::read_to_string(metrics).unwrap();
    assert_eq!(sample(&text, "eb_cvmfs_last_result"), Some(0.0));
    assert_eq!(sample(&text, "eb_cvmfs_exit_code"), Some(124.0));
    assert_eq!(sample(&text, "eb_cvmfs_aborts"), Some(0.0));
    assert!(
        sample(&text, "eb_cvmfs_last_publish_timestamp_seconds").unwrap() > 0.0,
        "{text}"
    );
    assert!(
        text.contains("eb_cvmfs_timeout{repository=\"repo.example\",phase=\"check\"} 1"),
        "{text}"
    );
}

#[test]
fn an_existing_transaction_is_refused_before_sync() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("STUB_IN_TRANSACTION", "1")
        .sync_output();
    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("already in a transaction"));
    assert_eq!(fixture.calls(), "list\n");
}

#[test]
fn cvmfs_setup_and_publish_failures_stop_at_the_failing_command() {
    let fixture = Fixture::new();
    let output = fixture.command().env("STUB_LIST_EXIT", "6").sync_output();
    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("cvmfs_server list failed"));
    assert_eq!(fixture.calls(), "list\n");

    std::fs::write(&fixture.calls, "").unwrap();
    let output = fixture
        .command()
        .env("STUB_TRANSACTION_EXIT", "7")
        .sync_output();
    assert_eq!(code(&output), 7);
    assert_eq!(fixture.calls(), "list\ntransaction repo.example\n");

    std::fs::write(&fixture.calls, "").unwrap();
    let output = fixture
        .command()
        .env("STUB_PUBLISH_EXIT", "8")
        .sync_output();
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
        .sync_output();
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
        .sync_output();
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
    let output = fixture.command().env("PATH", empty).sync_output();
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
        .sync_output();
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
        .sync_output();
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
        .sync_output();
    assert_eq!(code(&output), 0);
    assert_eq!(output.stdout, historical_dry_run(sync));
    assert_eq!(output.stderr, b"");

    let fixture = Fixture::new();
    let output = fixture.command().sync_output();
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
    let output = fixture.command_with_metrics(Some(&metrics)).sync_output();
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
fn absent_timeout_options_preserve_stdout_stderr_and_exit_status_bytes() {
    let fixture = Fixture::new();
    let output = fixture.command().sync_output();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"");
    assert_eq!(fixture.calls().as_bytes(), HISTORICAL_CALLS);

    let sync = "/opt/site/bin/sync-module-cache";
    let output = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .args(["--dry-run", "repo.example", "--", sync, "--delete"])
        .sync_output();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, historical_dry_run(sync));
    assert_eq!(output.stderr, b"");
}

#[test]
fn archive_manifest_after_the_separator_stays_a_sync_argument() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .arg("--archive-manifest")
        .arg("ingest.json")
        .sync_output();
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
        .sync_status();
    assert!(status.success());
    let manifest = manifest_for(&fixture, "cache.tar", "i32");

    let output = fixture.command_with(Some(&manifest), None).sync_output();
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
        .sync_output();
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
        .sync_status();
    assert!(status.success());
    let manifest = manifest_for(&fixture, "cache.tar", "i32");

    let output = fixture.command_with(Some(&manifest), None).sync_output();
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
        .sync_output();
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
        .sync_status();
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
        .sync_output();
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
        .sync_output();
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
        .sync_output();
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
        .sync_output();
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
        .sync_output();
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
        .sync_output();
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
        .sync_output();
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
        vec!["--sync-timeout"],
        vec!["--sync-timeout", "0", "repo.example", "--", "sync"],
        vec!["--sync-timeout", "1.5", "repo.example", "--", "sync"],
        vec!["--sync-timeout", "2147483648", "repo.example", "--", "sync"],
        vec![
            "--sync-timeout",
            "18446744073709551615",
            "repo.example",
            "--",
            "sync",
        ],
        vec![
            "--sync-timeout",
            "1",
            "--sync-timeout=2",
            "repo.example",
            "--",
            "sync",
        ],
        vec!["--publish-timeout"],
        vec!["--publish-timeout=0", "repo.example", "--", "sync"],
        vec![
            "--publish-timeout",
            "1",
            "--publish-timeout=2",
            "repo.example",
            "--",
            "sync",
        ],
        vec!["--check-timeout", "1", "repo.example", "--", "sync"],
        vec!["--check", "--check-timeout=0", "repo.example", "--", "sync"],
        vec![
            "--check",
            "--check-timeout",
            "1",
            "--check-timeout=2",
            "repo.example",
            "--",
            "sync",
        ],
        vec!["--verify-timeout", "1", "repo.example", "--", "sync"],
        vec![
            "--verify-cmd",
            "verify",
            "--verify-timeout=0",
            "repo.example",
            "--",
            "sync",
        ],
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
            .sync_output();
        assert_eq!(code(&output), 2);
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn concurrent_fixture_exec_writes_do_not_hit_text_file_busy() {
    let workers: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(|| {
                for _ in 0..20 {
                    let fixture = Fixture::new();
                    let output = fixture.command().sync_output();
                    assert_eq!(code(&output), 0, "{}", stderr(&output));
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
}

#[test]
fn cvmfs_dry_run_succeeds_when_stdout_consumer_closes() {
    let mut fds = [0; 2];
    unsafe {
        assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
        assert_eq!(libc::close(fds[0]), 0);
    }
    let closed = unsafe { <std::process::Stdio as std::os::fd::FromRawFd>::from_raw_fd(fds[1]) };
    let o = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .args(["--dry-run", "repo.example", "--", "sync"])
        .stdout(closed)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[test]
fn archive_manifest_accepts_equals_form_and_rejects_empty_or_duplicate() {
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
        .sync_status();
    assert!(status.success());
    let manifest = manifest_for(&fixture, "cache.tar", "i32");

    let output = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .env("PATH", &fixture.bin)
        .arg(format!("--archive-manifest={}", manifest.display()))
        .args(["--dry-run", "repo.example", "--"])
        .arg(&fixture.sync)
        .sync_output();
    assert_eq!(code(&output), 0, "{}", stderr(&output));

    let empty = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .env("PATH", &fixture.bin)
        .args(["--archive-manifest=", "--dry-run", "repo.example", "--"])
        .arg(&fixture.sync)
        .sync_output();
    assert_eq!(code(&empty), 2);
    assert!(stderr(&empty).contains("`--archive-manifest` needs a path"));

    let dup = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .env("PATH", &fixture.bin)
        .args([
            &format!("--archive-manifest={}", manifest.display()),
            "--archive-manifest",
            "other.json",
            "repo.example",
            "--",
        ])
        .arg(&fixture.sync)
        .sync_output();
    assert_eq!(code(&dup), 2);
    assert!(stderr(&dup).contains("`--archive-manifest` was given twice"));
}

#[test]
fn metrics_file_equals_separator_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .current_dir(dir.path())
        .args([
            "--dry-run",
            "--metrics-file=--",
            "repo.example",
            "--",
            "sync",
        ])
        .sync_output();
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("--metrics-file requires PATH"),
        "{}",
        stderr(&output)
    );
    assert!(output.stdout.is_empty());
    assert!(!dir.path().join("--").exists());
}

#[test]
fn value_options_do_not_consume_the_next_long_option() {
    let fixture = Fixture::new();
    let cases = [
        (
            &["--metrics-file", "--check"][..],
            "--metrics-file requires PATH",
        ),
        (&["--tag", "--check"][..], "`--tag` requires NAME"),
        (
            &["--tag", "release", "--tag-description", "--check"][..],
            "`--tag-description` requires TEXT",
        ),
        (
            &["--verify-cmd", "--check"][..],
            "`--verify-cmd` requires CMD",
        ),
        (
            &["--archive-manifest", "--check"][..],
            "`--archive-manifest` needs a path",
        ),
        (
            &["--sync-timeout", "--check"][..],
            "`--sync-timeout` requires SEC",
        ),
        (
            &["--verify-timeout", "--check"][..],
            "`--verify-timeout` requires SEC",
        ),
        (
            &["--publish-timeout", "--check"][..],
            "`--publish-timeout` requires SEC",
        ),
        (
            &["--check-timeout", "--check"][..],
            "`--check-timeout` requires SEC",
        ),
    ];

    for (args, message) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
            .env("PATH", &fixture.bin)
            .current_dir(fixture.dir.path())
            .arg("--dry-run")
            .args(args)
            .args(["repo.example", "--"])
            .arg(&fixture.sync)
            .sync_output();
        assert_eq!(code(&output), 2, "{args:?}: {}", stderr(&output));
        assert_eq!(output.stdout, b"", "{args:?}");
        assert_eq!(
            output.stderr,
            format!("eb-cvmfs: {message}\n").as_bytes(),
            "{args:?}"
        );
    }
    assert!(!fixture.dir.path().join("--check").exists());
}
